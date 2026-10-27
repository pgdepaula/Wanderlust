// Synthetic cache-save enumeration client; no game content.
#include <switch.h>
#include <string.h>
static void check(Result r) { if(R_FAILED(r)){svcBreak(0,r,0);svcExitProcess();} }
void __appInit(void) {}
void __appExit(void) {}
int main(void) {
    hosversionSet(MAKEHOSVERSION(16,0,0)); // Explicit wire revision; appInit is intentionally minimal.
    check(smInitialize());check(fsInitialize());
    for(unsigned iteration=0;iteration<2;iteration++) {
        Service reader={0};
        check(serviceDispatch(fsGetServiceSession(),62,.out_num_objects=1,.out_objects=&reader));
        FsSaveDataInfo entries[2];memset(entries,0x55,sizeof entries);s64 count=-1;
        check(serviceDispatchOut(&reader,0,count,
            .buffer_attrs={SfBufferAttr_HipcMapAlias|SfBufferAttr_Out},
            .buffers={{entries,sizeof entries}}));
        if(count!=0)check(1);
        for(unsigned i=0;i<sizeof entries;i++)if(((const u8*)entries)[i]!=0x55)check(1);
        serviceClose(&reader);
    }
    FsFileSystem save={0};FsSaveDataAttribute attr={0};attr.save_data_type=FsSaveDataType_Account;attr.uid.uid[0]=1;
    check(fsOpenSaveDataFileSystem(&save,FsSaveDataSpaceId_User,&attr));
    FsFile file={0};FsDir directory={0};FsDirEntryType type;
    if(fsFsOpenFile(&save,"/missing.dat",FsOpenMode_Read,&file)!=0x202)check(1);
    if(fsFsGetEntryType(&save,"/missing.dat",&type)!=0x202)check(1);
    if(fsFsOpenDirectory(&save,"/missing",FsDirOpenMode_ReadDirs,&directory)!=0x202)check(1);
    check(fsFsCreateDirectory(&save,"/storage"));
    check(fsFsCreateFile(&save,"/storage/key",4,0));
    check(fsFsOpenFile(&save,"/storage/key",FsOpenMode_Write,&file));
    check(fsFileWrite(&file,0,"REAL",4,FsWriteOption_Flush));fsFileClose(&file);
    FsFileSystem alias={0};check(fsOpenReadOnlySaveDataFileSystem(&alias,FsSaveDataSpaceId_User,&attr));
    check(fsFsOpenFile(&alias,"/storage/key",FsOpenMode_Read,&file));
    char key[4]={0};u64 key_read=0;check(fsFileRead(&file,0,key,4,0,&key_read));
    if(key_read!=4||memcmp(key,"REAL",4))check(1);fsFileClose(&file);fsFsClose(&alias);
    FsSaveDataAttribute foreign=attr;foreign.application_id=0xfffffffffffffff0ULL;
    if(fsOpenReadOnlySaveDataFileSystem(&alias,FsSaveDataSpaceId_User,&foreign)!=0x7d402)check(1);
    fsFsClose(&save);
    Service applet={0},proxy={0},functions={0};
    check(smGetService(&applet,"appletOE"));check(serviceConvertToDomain(&applet));
    u64 reserved=0;
    check(serviceDispatchIn(&applet,0,reserved,.in_send_pid=true,
        .in_num_handles=1,.in_handles={CUR_PROCESS_HANDLE},
        .out_num_objects=1,.out_objects=&proxy));
    check(serviceDispatch(&proxy,20,.out_num_objects=1,.out_objects=&functions));
    struct { u16 index;u8 pad[6];s64 size,journal; } create={7,{0},1048576,4096};
    struct { u32 target,pad;u64 required; } result={0};
    check(serviceDispatchInOut(&functions,27,create,result));
    if(result.target!=1||result.required!=0)check(1);
    FsSaveDataAttribute cache_attr={0};cache_attr.save_data_type=FsSaveDataType_Cache;cache_attr.save_data_index=7;
    FsFileSystem cache={0};check(fsOpenSaveDataFileSystem(&cache,FsSaveDataSpaceId_User,&cache_attr));
    check(fsFsCreateDirectory(&cache,"/storage"));
    check(fsFsCreateFile(&cache,"/storage/test.dat",8,0));
    check(fsFsOpenFile(&cache,"/storage/test.dat",FsOpenMode_Read|FsOpenMode_Write,&file));
    const u8 payload[8]={0x19,0x32,0x7f,0x80,0,0xff,0x52,0x92};u8 output[16];memset(output,0x55,sizeof output);u64 read=0;
    check(fsFileWrite(&file,0,payload,8,FsWriteOption_Flush));check(fsFileRead(&file,0,output,16,0,&read));
    if(read!=8||memcmp(output,payload,8)||output[8]!=0x55)check(1);
    fsFileClose(&file);check(fsFsCommit(&cache));
    check(fsFsOpenFile(&cache,"/storage/test.dat",FsOpenMode_Read,&file));
    s64 file_size=0;check(fsFileGetSize(&file,&file_size));if(file_size!=8)check(1);fsFileClose(&file);
    check(fsFsRenameFile(&cache,"/storage/test.dat","/storage/renamed.dat"));
    check(fsFsOpenDirectory(&cache,"/storage",FsDirOpenMode_ReadFiles,&directory));
    s64 count=-1;check(fsDirGetEntryCount(&directory,&count));if(count!=1)check(1);
    FsDirectoryEntry entry={0};check(fsDirRead(&directory,&count,1,&entry));
    if(count!=1||strcmp(entry.name,"renamed.dat")||entry.file_size!=8||entry.type!=FsDirEntryType_File)check(1);
    check(fsDirRead(&directory,&count,1,&entry));if(count!=0)check(1);fsDirClose(&directory);
    check(fsFsRenameDirectory(&cache,"/storage","/moved"));
    check(fsFsGetEntryType(&cache,"/moved/renamed.dat",&type));if(type!=FsDirEntryType_File)check(1);
    check(fsFsCleanDirectoryRecursively(&cache,"/moved"));
    if(fsFsGetEntryType(&cache,"/moved/renamed.dat",&type)!=0x202)check(1);
    check(fsFsDeleteDirectory(&cache,"/moved"));
    check(fsFsCreateDirectory(&cache,"/again"));check(fsFsCreateDirectory(&cache,"/again/child"));
    check(fsFsDeleteDirectoryRecursively(&cache,"/again"));
    Service reader={0};check(serviceDispatch(fsGetServiceSession(),62,.out_num_objects=1,.out_objects=&reader));
    FsSaveDataInfo info={0};check(serviceDispatchOut(&reader,0,count,.buffer_attrs={SfBufferAttr_HipcMapAlias|SfBufferAttr_Out},.buffers={{&info,sizeof info}}));
    if(count!=1||info.save_data_index!=7||info.save_data_type!=FsSaveDataType_Cache||info.size!=1052672)check(1);
    serviceClose(&reader);fsFsClose(&cache);serviceClose(&functions);serviceClose(&proxy);serviceClose(&applet);
    fsExit();smExit();svcOutputDebugString("FSCACHE_NATIVE_OK",17);svcExitProcess();
}
